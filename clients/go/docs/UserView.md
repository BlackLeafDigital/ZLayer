# UserView

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**DisplayName** | **string** |  | 
**Email** | **string** |  | 
**Id** | **string** |  | 
**IsActive** | **bool** |  | 
**LastLoginAt** | Pointer to **NullableString** |  | [optional] 
**Role** | **string** |  | 

## Methods

### NewUserView

`func NewUserView(displayName string, email string, id string, isActive bool, role string, ) *UserView`

NewUserView instantiates a new UserView object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewUserViewWithDefaults

`func NewUserViewWithDefaults() *UserView`

NewUserViewWithDefaults instantiates a new UserView object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetDisplayName

`func (o *UserView) GetDisplayName() string`

GetDisplayName returns the DisplayName field if non-nil, zero value otherwise.

### GetDisplayNameOk

`func (o *UserView) GetDisplayNameOk() (*string, bool)`

GetDisplayNameOk returns a tuple with the DisplayName field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetDisplayName

`func (o *UserView) SetDisplayName(v string)`

SetDisplayName sets DisplayName field to given value.


### GetEmail

`func (o *UserView) GetEmail() string`

GetEmail returns the Email field if non-nil, zero value otherwise.

### GetEmailOk

`func (o *UserView) GetEmailOk() (*string, bool)`

GetEmailOk returns a tuple with the Email field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetEmail

`func (o *UserView) SetEmail(v string)`

SetEmail sets Email field to given value.


### GetId

`func (o *UserView) GetId() string`

GetId returns the Id field if non-nil, zero value otherwise.

### GetIdOk

`func (o *UserView) GetIdOk() (*string, bool)`

GetIdOk returns a tuple with the Id field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetId

`func (o *UserView) SetId(v string)`

SetId sets Id field to given value.


### GetIsActive

`func (o *UserView) GetIsActive() bool`

GetIsActive returns the IsActive field if non-nil, zero value otherwise.

### GetIsActiveOk

`func (o *UserView) GetIsActiveOk() (*bool, bool)`

GetIsActiveOk returns a tuple with the IsActive field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetIsActive

`func (o *UserView) SetIsActive(v bool)`

SetIsActive sets IsActive field to given value.


### GetLastLoginAt

`func (o *UserView) GetLastLoginAt() string`

GetLastLoginAt returns the LastLoginAt field if non-nil, zero value otherwise.

### GetLastLoginAtOk

`func (o *UserView) GetLastLoginAtOk() (*string, bool)`

GetLastLoginAtOk returns a tuple with the LastLoginAt field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetLastLoginAt

`func (o *UserView) SetLastLoginAt(v string)`

SetLastLoginAt sets LastLoginAt field to given value.

### HasLastLoginAt

`func (o *UserView) HasLastLoginAt() bool`

HasLastLoginAt returns a boolean if a field has been set.

### SetLastLoginAtNil

`func (o *UserView) SetLastLoginAtNil(b bool)`

 SetLastLoginAtNil sets the value for LastLoginAt to be an explicit nil

### UnsetLastLoginAt
`func (o *UserView) UnsetLastLoginAt()`

UnsetLastLoginAt ensures that no value is present for LastLoginAt, not even an explicit nil
### GetRole

`func (o *UserView) GetRole() string`

GetRole returns the Role field if non-nil, zero value otherwise.

### GetRoleOk

`func (o *UserView) GetRoleOk() (*string, bool)`

GetRoleOk returns a tuple with the Role field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetRole

`func (o *UserView) SetRole(v string)`

SetRole sets Role field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


