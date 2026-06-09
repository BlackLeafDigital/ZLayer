# UlimitSpec

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Hard** | Pointer to **int64** | Hard limit. | [optional] 
**Soft** | Pointer to **int64** | Soft limit. | [optional] 

## Methods

### NewUlimitSpec

`func NewUlimitSpec() *UlimitSpec`

NewUlimitSpec instantiates a new UlimitSpec object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewUlimitSpecWithDefaults

`func NewUlimitSpecWithDefaults() *UlimitSpec`

NewUlimitSpecWithDefaults instantiates a new UlimitSpec object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetHard

`func (o *UlimitSpec) GetHard() int64`

GetHard returns the Hard field if non-nil, zero value otherwise.

### GetHardOk

`func (o *UlimitSpec) GetHardOk() (*int64, bool)`

GetHardOk returns a tuple with the Hard field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetHard

`func (o *UlimitSpec) SetHard(v int64)`

SetHard sets Hard field to given value.

### HasHard

`func (o *UlimitSpec) HasHard() bool`

HasHard returns a boolean if a field has been set.

### GetSoft

`func (o *UlimitSpec) GetSoft() int64`

GetSoft returns the Soft field if non-nil, zero value otherwise.

### GetSoftOk

`func (o *UlimitSpec) GetSoftOk() (*int64, bool)`

GetSoftOk returns a tuple with the Soft field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetSoft

`func (o *UlimitSpec) SetSoft(v int64)`

SetSoft sets Soft field to given value.

### HasSoft

`func (o *UlimitSpec) HasSoft() bool`

HasSoft returns a boolean if a field has been set.


[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


