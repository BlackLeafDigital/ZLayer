# GroupMembersResponse

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**GroupId** | **string** | Group id. | 
**Members** | **[]string** | List of user ids in the group. | 

## Methods

### NewGroupMembersResponse

`func NewGroupMembersResponse(groupId string, members []string, ) *GroupMembersResponse`

NewGroupMembersResponse instantiates a new GroupMembersResponse object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewGroupMembersResponseWithDefaults

`func NewGroupMembersResponseWithDefaults() *GroupMembersResponse`

NewGroupMembersResponseWithDefaults instantiates a new GroupMembersResponse object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetGroupId

`func (o *GroupMembersResponse) GetGroupId() string`

GetGroupId returns the GroupId field if non-nil, zero value otherwise.

### GetGroupIdOk

`func (o *GroupMembersResponse) GetGroupIdOk() (*string, bool)`

GetGroupIdOk returns a tuple with the GroupId field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetGroupId

`func (o *GroupMembersResponse) SetGroupId(v string)`

SetGroupId sets GroupId field to given value.


### GetMembers

`func (o *GroupMembersResponse) GetMembers() []string`

GetMembers returns the Members field if non-nil, zero value otherwise.

### GetMembersOk

`func (o *GroupMembersResponse) GetMembersOk() (*[]string, bool)`

GetMembersOk returns a tuple with the Members field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetMembers

`func (o *GroupMembersResponse) SetMembers(v []string)`

SetMembers sets Members field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


