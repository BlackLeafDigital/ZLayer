# NetworkSummary

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**CidrCount** | **int32** |  | 
**Description** | Pointer to **NullableString** |  | [optional] 
**MemberCount** | **int32** |  | 
**Name** | **string** |  | 
**RuleCount** | **int32** |  | 

## Methods

### NewNetworkSummary

`func NewNetworkSummary(cidrCount int32, memberCount int32, name string, ruleCount int32, ) *NetworkSummary`

NewNetworkSummary instantiates a new NetworkSummary object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewNetworkSummaryWithDefaults

`func NewNetworkSummaryWithDefaults() *NetworkSummary`

NewNetworkSummaryWithDefaults instantiates a new NetworkSummary object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetCidrCount

`func (o *NetworkSummary) GetCidrCount() int32`

GetCidrCount returns the CidrCount field if non-nil, zero value otherwise.

### GetCidrCountOk

`func (o *NetworkSummary) GetCidrCountOk() (*int32, bool)`

GetCidrCountOk returns a tuple with the CidrCount field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetCidrCount

`func (o *NetworkSummary) SetCidrCount(v int32)`

SetCidrCount sets CidrCount field to given value.


### GetDescription

`func (o *NetworkSummary) GetDescription() string`

GetDescription returns the Description field if non-nil, zero value otherwise.

### GetDescriptionOk

`func (o *NetworkSummary) GetDescriptionOk() (*string, bool)`

GetDescriptionOk returns a tuple with the Description field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetDescription

`func (o *NetworkSummary) SetDescription(v string)`

SetDescription sets Description field to given value.

### HasDescription

`func (o *NetworkSummary) HasDescription() bool`

HasDescription returns a boolean if a field has been set.

### SetDescriptionNil

`func (o *NetworkSummary) SetDescriptionNil(b bool)`

 SetDescriptionNil sets the value for Description to be an explicit nil

### UnsetDescription
`func (o *NetworkSummary) UnsetDescription()`

UnsetDescription ensures that no value is present for Description, not even an explicit nil
### GetMemberCount

`func (o *NetworkSummary) GetMemberCount() int32`

GetMemberCount returns the MemberCount field if non-nil, zero value otherwise.

### GetMemberCountOk

`func (o *NetworkSummary) GetMemberCountOk() (*int32, bool)`

GetMemberCountOk returns a tuple with the MemberCount field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetMemberCount

`func (o *NetworkSummary) SetMemberCount(v int32)`

SetMemberCount sets MemberCount field to given value.


### GetName

`func (o *NetworkSummary) GetName() string`

GetName returns the Name field if non-nil, zero value otherwise.

### GetNameOk

`func (o *NetworkSummary) GetNameOk() (*string, bool)`

GetNameOk returns a tuple with the Name field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetName

`func (o *NetworkSummary) SetName(v string)`

SetName sets Name field to given value.


### GetRuleCount

`func (o *NetworkSummary) GetRuleCount() int32`

GetRuleCount returns the RuleCount field if non-nil, zero value otherwise.

### GetRuleCountOk

`func (o *NetworkSummary) GetRuleCountOk() (*int32, bool)`

GetRuleCountOk returns a tuple with the RuleCount field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetRuleCount

`func (o *NetworkSummary) SetRuleCount(v int32)`

SetRuleCount sets RuleCount field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


